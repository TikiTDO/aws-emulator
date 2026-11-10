// SPDX-License-Identifier: CC0-1.0

//! @op PollForJobs
//!
//! ## What this operation serves, and to whom
//!
//! `PollForJobs` is the CUSTOM action worker's end of the service: a job worker the
//! ACCOUNT runs polls CodePipeline for the jobs its own action type has waiting.
//! "PollForJobs is valid only for action types with `Custom` in the owner field. If the
//! action type contains `AWS` or `ThirdParty` in the owner field, the PollForJobs action
//! returns an error." Four rules follow from that, and this handler is all four:
//!
//! 1. **Only a custom action is polled this way.** An `actionTypeId` naming any other
//!    `ActionOwner` is refused `ValidationException` (HTTP 400) before anything is
//!    reported: an `AWS` action is run by AWS, and a `ThirdParty` one by its partner,
//!    whose jobs `PollForThirdPartyJobs` serves.
//! 2. **The action type has to exist.** A `Custom` action type exists only because this
//!    account registered it (`CreateCustomActionType`), so an id the registry does not
//!    hold is refused `ActionTypeNotFoundException` (HTTP 400) — the built-in catalog
//!    holds no `Custom` entry, and a job waiting is not what makes an action type
//!    exist. An id this account registered and then DELETED still resolves here: the
//!    delete marks the custom action deleted, so the worker still polling it is
//!    answered — with no jobs, because no job of a deleted custom action is handed out
//!    again
//!    ([`crate::state::ComAmazonawsCodepipelineCodepipeline20150709State::pollable_action_type`]).
//! 3. **`queryParam` selects.** "For an action type with no queryable properties, this
//!    value must be null or an empty map. For an action type with a queryable property,
//!    you must supply that property as a key in the map. Only jobs whose action
//!    configuration matches the mapped value are returned." A poll that disagrees with
//!    the registered action type's queryable property is refused
//!    `ValidationException`; one that agrees selects the jobs whose action
//!    configuration maps that property to the value it supplied.
//! 4. **A poll CLAIMS what it reports.** `Job$nonce` is "a system-generated random
//!    number that CodePipeline uses to ensure that the job is being worked on by only
//!    one job worker", so a job this poll reports is handed to the worker that polled:
//!    a fresh nonce is minted for it and it is withheld from later polls until that
//!    claim lapses ([`crate::state::JOB_POLL_LEASE`]), after which it is available
//!    again — with a NEW nonce, so the one the lapsed claim published no longer reaches
//!    it. Everything else about the job is REPORTED from the store, not from the poll.
//!
//! The jobs themselves are queued by the execution plane: when an execution reaches an
//! action whose action type is `Custom`, CodePipeline cannot run it, so
//! [`crate::state::driver`] queues the job this operation hands out and leaves the
//! action `InProgress` until a worker reports its result. A poll therefore first
//! materialises the execution steps the CLOCK has already made due (the sanctioned "a
//! call may write only to materialise a change the clock already made due" case, the
//! same one `GetPipelineState` performs) — for the pipelines with an execution that can
//! still move, since the request names no pipeline of its own — and then reports what is
//! waiting. A custom action type with nothing waiting is answered with no jobs rather
//! than with an invented one.
//!
//! `JobData$artifactCredentials` — the "temporary credentials for the S3 bucket used to
//! store artifacts" the doc below mentions — stays ABSENT, for the reason
//! [`crate::state::JobRecord::to_data`] records: it is an STS session SCOPED to the
//! artifact bucket, this emulator's credential store holds no policies to scope one
//! with, and a job is released by paths with no credential store in reach, so nothing
//! would ever revoke it. The artifact LOCATIONS a worker needs are reported in full.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `PollForJobs::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `PollForJobs`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codepipeline-codepipeline-20150709/smithy-model/com-amazonaws-codepipeline-codepipeline-20150709.json")
//! ```
//!
//! Returns information about any jobs for CodePipeline to act on. PollForJobs is valid only
//! for action types with "Custom" in the owner field. If the action type contains AWS or
//! ThirdParty in the owner field, the PollForJobs action returns an error. When this API is
//! called, CodePipeline returns temporary credentials for the S3 bucket used to store
//! artifacts for the pipeline, if the action requires access to that S3 bucket for input or
//! output artifacts. This API also returns any secret values defined for the action.
//!
//! ## Input
//!
//! * `actionTypeId` (ActionTypeId) **required** — Represents information about an action
//!   type.
//! * `maxBatchSize` (MaxBatchSize, range 1..) — The maximum number of jobs to return in a
//!   poll for jobs call.
//! * `queryParam` (QueryParamMap, len 0..1) — A map of property names and values. For an
//!   action type with no queryable properties, this value must be null or an empty map. For
//!   an action type with a queryable property, you must supply that property as a key in the
//!   map. Only jobs whose action configuration matches the mapped value are returned.
//!
//! ## Output
//!
//! * `jobs` (JobList) — Information about the jobs to take action on.
//!
//! ## Errors
//!
//! * `ActionTypeNotFoundException` (HTTP 400) — The specified action type cannot be found.
//! * `ValidationException` (HTTP 400) — The validation was specified in an invalid format.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `PollForJobsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codepipeline-codepipeline-20150709.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PollForJobsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.codepipeline#PollForJobs")]
#[derive(Debug)]
pub enum PollForJobsErrorModeled {
    ActionTypeNotFoundException(ActionTypeNotFoundException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::poll_for_jobs::PollForJobsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PollForJobsError = crate::scaffolded::Scaffolded<PollForJobsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PollForJobsErrorModeled> for PollForJobsError {
    fn from(modeled: PollForJobsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The refusal for an `actionTypeId` whose owner is not `Custom`, built from the four
/// identifying fields the request named.
fn not_a_custom_action_message(id: &ActionTypeId) -> String {
    format!(
        "PollForJobs is valid only for an action type whose owner is {}, but \
         ActionType.Category='{}' ActionType.Owner='{}' ActionType.Provider='{}' \
         ActionType.Version='{}'",
        crate::state::ACTION_OWNER_CUSTOM,
        id.category,
        id.owner,
        id.provider,
        id.version
    )
}

/// The `queryParam` entry a poll supplied, checked against the queryable configuration
/// property the registered action type declares, if any.
///
/// Both halves of what the model states about the member, in one place: an action type
/// with no queryable property takes no `queryParam` at all, and one that declares a
/// queryable property is polled BY it. The refusal names the property, so a caller can
/// correct the call from the message alone.
fn selector<'a>(
    queryable: Option<&str>,
    query: &'a [(String, String)],
) -> Result<Option<(&'a str, &'a str)>, String> {
    match (queryable, query) {
        (None, []) => Ok(None),
        (None, [(name, _), ..]) => Err(format!(
            "queryParam: the action type declares no queryable configuration property, \
             so queryParam must be null or an empty map, but it names {name}"
        )),
        (Some(property), []) => Err(format!(
            "queryParam: the action type declares the queryable configuration property \
             {property}, which a poll must supply as a key in queryParam"
        )),
        (Some(property), [(name, value), ..]) if name == property => {
            Ok(Some((name.as_str(), value.as_str())))
        }
        (Some(property), [(name, _), ..]) => Err(format!(
            "queryParam: {name} is not a queryable configuration property of the action \
             type, whose queryable property is {property}"
        )),
    }
}

/// The `PollForJobs` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PollForJobs<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PollForJobs<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PollForJobs<Eng>
{
    type Input = PollForJobsInput;
    type Output = PollForJobsOutput;
    type Error = PollForJobsError;

    const NAME: &'static str = "PollForJobs";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::POLL_FOR_JOBS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::POLL_FOR_JOBS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::require_state(&*self.engine)?;
        let action_type = &input.action_type_id;

        // Rule 1: only a custom action's jobs are polled for here.
        if action_type.owner != crate::state::ACTION_OWNER_CUSTOM {
            return Err(PollForJobsErrorModeled::ValidationException(
                ValidationException {
                    message: Some(not_a_custom_action_message(action_type)),
                },
            )
            .into());
        }

        // Rule 2: the action type has to be one this account registered — or one it
        // registered and has since deleted, which a worker still polling is answered
        // rather than refused — and rule 3 is checked against that registration's
        // queryable property. At most one property of an action type is queryable (the
        // registration refuses more).
        let (resolution, queryable) = {
            let store = state.lock();
            let Some((resolution, registered)) =
                store.pollable_action_type(&crate::state::ActionTypeKey::from_id(action_type))
            else {
                return Err(PollForJobsErrorModeled::ActionTypeNotFoundException(
                    ActionTypeNotFoundException {
                        message: Some(crate::state::action_type_not_found_message(action_type)),
                    },
                )
                .into());
            };
            let queryable = registered
                .configuration_properties
                .iter()
                .find(|property| property.queryable)
                .map(|property| property.name.clone());
            (resolution, queryable)
        };

        // Rule 3: what this poll selects on, or the refusal for a `queryParam` the
        // action type cannot be polled by. The member is modelled `len 0..1`, so there
        // is at most one pair to consider.
        let query: Vec<(String, String)> = input
            .query_param
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let selected = selector(queryable.as_deref(), &query).map_err(|message| {
            PollForJobsError::from(PollForJobsErrorModeled::ValidationException(
                ValidationException {
                    message: Some(message),
                },
            ))
        })?;

        // A deleted registration is answered and nothing more: the custom action is
        // marked deleted, so no job of it is handed to a worker again and the one still
        // polling drains. The request is still judged by rules 1 and 3 above, so a poll
        // that could not be served while the registration was held is not served now
        // either.
        if resolution == crate::state::PollResolution::Deleted {
            return Ok(PollForJobsOutput {
                jobs: Some(Vec::new()),
            });
        }

        // A job is queued when an execution REACHES a custom action, which is a step
        // the clock owes (see `state::driver`). The request names no pipeline, so the
        // pipelines with an execution that can still move are the ones walked; one
        // whose executions have all settled is owed nothing.
        let pipelines = state.lock().pipelines_with_live_executions();
        for pipeline in pipelines {
            crate::state::advance(&*self.engine, ctx, &pipeline).await;
        }

        // Rule 4: the jobs still waiting for this action type, oldest first, at most
        // `maxBatchSize` of them, each handed to this worker with a fresh nonce.
        let now = ::std::time::SystemTime::now();
        let leased = {
            let mut store = state.lock();
            // A job whose action is no longer waiting for a worker (it was abandoned by
            // a stop, or it reached an outcome) is not work any more: it is forgotten
            // rather than handed out.
            store.retire_finished_jobs();
            store.lease_jobs(
                action_type,
                crate::state::poll_limit(input.max_batch_size),
                selected,
                now,
            )
        };

        // Handing a job out is what mints the credentials its worker reads and writes the
        // pipeline's artifacts with (`state::mint_artifact_credentials`), for a job that
        // carries artifacts at all. Two rules keep the credential store bounded, and this
        // is the one path that applies them: a job's credentials are minted ONCE, so a
        // re-dispatch of a job that still carries them mints nothing; and the credentials
        // the clock has already ended are released here first — a release owed at the same
        // instant whether or not anybody polled. A credential the store declines to record
        // (the job was retired meanwhile, or the store is at its bound) is released again
        // rather than left behind.
        for access_key_id in state.lock().due_artifact_credentials(now) {
            let _ = ctx.credential_store().delete(&access_key_id);
        }
        let mut jobs = Vec::with_capacity(leased.len());
        for mut job in leased {
            if job.needs_artifact_credentials() && job.artifact_credentials.is_none() {
                if let Some(credentials) = crate::state::mint_artifact_credentials(ctx) {
                    let recorded = state.lock().set_artifact_credentials(
                        &job.job_id,
                        credentials.clone(),
                        now + crate::state::ARTIFACT_CREDENTIAL_LIFETIME,
                    );
                    if recorded {
                        job.artifact_credentials = Some(credentials);
                    } else {
                        let _ = ctx.credential_store().delete(&credentials.access_key_id);
                    }
                }
            }
            jobs.push(job.to_job());
        }

        Ok(PollForJobsOutput {
            // The list is always reported, empty when nothing is waiting: a worker
            // loop reads `jobs` on every answer, so an empty poll is an empty LIST
            // rather than a missing member.
            jobs: Some(jobs),
        })
    }
}

