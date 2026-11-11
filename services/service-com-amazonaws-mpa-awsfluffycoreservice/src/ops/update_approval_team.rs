// SPDX-License-Identifier: CC0-1.0

//! @op UpdateApprovalTeam
//! Records the supplied `Description`, `ApprovalStrategy` and/or `Approvers` as a NEW
//! VERSION of the stored team and returns that version's id.
//!
//! "Updates require team approval. Updates to an active team must be approved by the
//! team": a change to the configuration an ACTIVE team's approvers approved is therefore
//! not applied where it is asked for. It is REQUESTED — the team keeps its description,
//! strategy, approvers and version id, reports `PENDING` with `UPDATE_PENDING_APPROVAL`,
//! and the requested version is what `GetApprovalTeam` reports in `PendingUpdate`, beside
//! the update session this operation puts the change to the team in
//! (`UpdateSessionArn`). The clock owes the rest: the approvers' approval
//! ([`crate::state::UPDATE_APPROVAL_DELAY`]), after which the team reports
//! `UPDATE_PENDING_ACTIVATION`, and the activation of the approved configuration
//! ([`crate::state::UPDATE_ACTIVATION_DELAY`]), at which point the requested version
//! becomes the team's live one and the team is `ACTIVE` again with nothing pending — and
//! the update session reports the action it approved as `EXECUTED`
//! ([`crate::state::Session::record_execution`]), because that activation is the change
//! being carried out.
//!
//! A team whose configuration has never been activated (a team still waiting on its
//! approvers' invitations), and one that is no longer active, has no approved
//! configuration to protect, so the change is applied in place and the new version is
//! live at once.
//!
//! * An ARN outside the caller's own account is refused with `AccessDeniedException`
//!   (403) BEFORE the lookup ([`crate::state::authorize_resource_arn`]); an ARN inside
//!   it that no stored team carries earns `ResourceNotFoundException` (404).
//! * A team already in `DELETING` cannot be changed: `ConflictException` (409). So is a
//!   second CHANGE to a team that already has a version awaiting its approvers' approval
//!   — one change at a time. A request that names no configuration member asks those
//!   approvers nothing, so it is not a second change and is served whatever version is
//!   outstanding.
//! * An `MofN` strategy asking for more approvals (M) than the team will have approvers
//!   (N) earns `ValidationException` (400) — the same unsatisfiable combination
//!   `CreateApprovalTeam` refuses, checked against the approver set the update produces.
//!   `Approvers` naming one identity twice is refused the same way.
//! * An approver whose `PrimaryIdentitySourceArn` is not an identity source's ARN
//!   ([`crate::state::malformed_approver_identity_source`]) earns the malformed-ARN
//!   refusal this service states once ([`crate::state::invalid_arn`],
//!   `ValidationException` 400): the member carries no ARN `@pattern`, so this is where
//!   that class is refused, and no approver is ever stored — or reported — with an
//!   identity source reference that addresses nothing.
//! * A request that would take the store past its session ceiling earns the modelled
//!   `ServiceQuotaExceededException` (402).
//!
//! An approver the request keeps (same `PrimaryIdentityId` from the same identity source)
//! keeps its `ApproverId` and its invitation response; one the request adds is newly
//! invited, so it is `PENDING` until the clock owes its response. Every refusal above is
//! decided BEFORE anything is written — the ids the added approvers carry are minted, and
//! the update session recorded, only once the request is accepted — so a refused request
//! leaves the store exactly as it found it and consumes no `ApproverId`.
//!
//! Every `UpdateActions` entry is APPLIED to the team the update produces: the single
//! modelled action ([`crate::state::SYNCHRONIZE_MFA_DEVICES_ACTION`]) synchronizes the
//! MFA devices of every approver on the team, which `GetApprovalTeam` reports per
//! approver in `MfaMethods`. It changes no member of the configuration the team approved
//! — it reads each approver's devices back off the identity source — so it takes effect
//! where it is asked for rather than waiting on an approval, including while a version is
//! awaiting the team's approval: a device belongs to the approver's identity, so the
//! records the live set and that version hold are synchronized alike. A request that
//! carries no field and no action to apply leaves the team — and therefore its version —
//! as it was.
//!
//! A call answers with ONE version id, and the team's live version id and
//! `LastUpdateTime` only ever move to the version a call announced: so where the change is
//! recorded for the approvers rather than applied, the announced version is the REQUESTED
//! one and the live version id and `LastUpdateTime` stay as the approvers left them,
//! whatever else the same request applies in place.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `UpdateApprovalTeam::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `UpdateApprovalTeam`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-mpa-awsfluffycoreservice/smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json")
//! ```
//!
//! Updates an approval team. You can request to update the team description, approval
//! threshold, and approvers in the team. Updates require team approval Updates to an active
//! team must be approved by the team.
//!
//! ## Input
//!
//! * `ApprovalStrategy` (ApprovalStrategy) — An ApprovalStrategy object. Contains details for
//!   how the team grants approval.
//! * `Approvers` (ApprovalTeamRequestApprovers, len 1..20) — An array of
//!   ApprovalTeamRequestApprover objects. Contains details for the approvers in the team.
//! * `Arn` (ApprovalTeamArn, len 20..2048, pattern
//!   ^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:approval-team/[a-zA-Z0-9._-]+$)
//!   **required** — Amazon Resource Name (ARN) for the team.
//! * `Description` (Description, len 1..256) — Description for the team.
//! * `UpdateActions` (UpdateActions, len 0..100) — A list of UpdateAction to perform when
//!   updating the team.
//!
//! ## Output
//!
//! * `VersionId` (String, len ..1000) — Version ID for the team that was created. When an
//!   approval team is updated, the version ID changes.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action. Check your permissions, and try again.
//! * `ConflictException` (HTTP 409) — The request cannot be completed because it conflicts
//!   with the current state of a resource.
//! * `InternalServerException` (HTTP 500) — The service encountered an internal error. Try
//!   your request again. If the problem persists, contact Amazon Web Services Support.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource doesn't exist. Check the
//!   resource ID, and try again.
//! * `ServiceQuotaExceededException` (HTTP 402) — The request exceeds the service quota for
//!   your account. Request a quota increase or reduce your request size.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints specified
//!   by an Amazon Web Services service.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateApprovalTeamError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateApprovalTeamError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.mpa#UpdateApprovalTeam")]
#[derive(Debug)]
pub enum UpdateApprovalTeamErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_approval_team::UpdateApprovalTeamError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateApprovalTeamError = crate::scaffolded::Scaffolded<UpdateApprovalTeamErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateApprovalTeamErrorModeled> for UpdateApprovalTeamError {
    fn from(modeled: UpdateApprovalTeamErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateApprovalTeam` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateApprovalTeam<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateApprovalTeam<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateApprovalTeam<Eng>
{
    type Input = UpdateApprovalTeamInput;
    type Output = UpdateApprovalTeamOutput;
    type Error = UpdateApprovalTeamError;

    const NAME: &'static str = "UpdateApprovalTeam";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_APPROVAL_TEAM_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_APPROVAL_TEAM_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The account boundary is evaluated BEFORE the resource is resolved: an ARN
        // outside the caller's account is refused with the modelled
        // `AccessDeniedException` (403) whatever is stored under it, so a client never
        // reads "the team is gone" for a team it simply cannot reach.
        crate::state::authorize_resource_arn(Self::NAME, &input.arn)
            .map_err(UpdateApprovalTeamErrorModeled::AccessDeniedException)?;
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdateApprovalTeamError::Modeled(
                UpdateApprovalTeamErrorModeled::InternalServerException(InternalServerException {
                    message: "the approval-team store is unavailable".to_string(),
                }),
            )
        })?;
        let mut state = store.lock();
        let now = ::std::time::SystemTime::now();
        state.materialize_due(now);

        // The one message this service answers every resource lookup that resolves to
        // nothing with (`crate::state::resource_not_found`).
        let not_found = || {
            UpdateApprovalTeamError::Modeled(
                UpdateApprovalTeamErrorModeled::ResourceNotFoundException(
                    crate::state::resource_not_found(),
                ),
            )
        };

        // Snapshot the stored team, so minting approver ids and the update session (both
        // of which borrow the store mutably) do not overlap the borrow of the team.
        let team_record = state.team(&input.arn).cloned().ok_or_else(not_found)?;
        let existing_approvers = team_record.approvers.clone();
        let status = team_record.status;
        let current_strategy = team_record.approval_strategy;
        let current_description = team_record.description.clone();
        let outstanding_update = team_record
            .outstanding_update()
            .map(|update| update.version_id.clone());

        if status == crate::state::TeamStatus::Deleting {
            return Err(UpdateApprovalTeamErrorModeled::ConflictException(ConflictException {
                message: crate::state::bounded_message(format!(
                    "Approval team '{}' is being deleted and cannot be updated.",
                    input.arn
                )),
            })
            .into());
        }

        // Whether this request asks for the team's CONFIGURATION to change. An update
        // action asks for no configuration member to change: it reads each approver's MFA
        // devices back off the identity source, which is not a change the team's approvers
        // have anything to approve.
        let changes_configuration = input.description.is_some()
            || input.approval_strategy.is_some()
            || input.approvers.is_some();

        // One change at a time: a team already waiting on its approvers has a version it
        // is moving to, and a second CHANGE would ask them to approve two. A request that
        // asks for no configuration member to change asks them nothing, so it is not a
        // second change — it is served, and what it does ask for takes effect where it was
        // asked for.
        if let Some(version_id) = outstanding_update.filter(|_| changes_configuration) {
            return Err(UpdateApprovalTeamErrorModeled::ConflictException(ConflictException {
                message: crate::state::bounded_message(format!(
                    "Approval team '{}' already has version '{version_id}' awaiting the team's \
                     approval, so it cannot be updated again.",
                    input.arn
                )),
            })
            .into());
        }

        if let Some(requested) = input.approvers.as_ref() {
            // `PrimaryIdentitySourceArn` is documented to carry the identity source's ARN
            // and the model gives it no pattern, so the boundary validator passes a value
            // that addresses no identity source: this service's own ARN rule refuses it
            // here, before the approver set is rebuilt, so the stored approver's identity
            // source is always one this service could have minted.
            if let Some(malformed) = crate::state::malformed_approver_identity_source(
                requested
                    .iter()
                    .map(|a| a.primary_identity_source_arn.as_str()),
            ) {
                return Err(
                    UpdateApprovalTeamErrorModeled::ValidationException(malformed).into()
                );
            }
            if let Some(repeated) = crate::state::repeated_identity(requested.iter().map(|a| {
                (
                    a.primary_identity_id.as_str(),
                    a.primary_identity_source_arn.as_str(),
                )
            })) {
                return Err(
                    UpdateApprovalTeamErrorModeled::ValidationException(repeated).into()
                );
            }
        }

        // Rebuilding the approver set MINTS an id for every approver the request adds,
        // which advances the store's approver sequence: it is a write, so it waits until
        // every refusal this request can earn has been decided. Until then the request is
        // measured by the approver set it NAMES.
        let requested_approver_count = input
            .approvers
            .as_ref()
            .map_or(existing_approvers.len(), Vec::len);

        let new_strategy = input.approval_strategy.as_ref().map(|strategy| match strategy {
            ApprovalStrategy::MofN(mof_n) => crate::state::StoredApprovalStrategy::MofN {
                min_approvals_required: mof_n.min_approvals_required,
            },
        });

        // M of N is checked against the set the update PRODUCES, not the one it replaces.
        let resulting_count = i32::try_from(requested_approver_count).unwrap_or(i32::MAX);
        let resulting_min = new_strategy
            .unwrap_or(current_strategy)
            .min_approvals_required();
        if resulting_min > resulting_count {
            return Err(UpdateApprovalTeamErrorModeled::ValidationException(ValidationException {
                message: format!(
                    "ApprovalStrategy.MofN.MinApprovalsRequired ({resulting_min}) must not exceed \
                     the number of Approvers ({resulting_count})."
                ),
            })
            .into());
        }

        // Each requested action is applied to the team this update PRODUCES, so an
        // approver the same request adds is synchronized along with the ones it keeps.
        let synchronize_mfa_devices = input.update_actions.as_ref().is_some_and(|actions| {
            actions
                .iter()
                .any(|action| action == crate::state::SYNCHRONIZE_MFA_DEVICES_ACTION)
        });

        // A change to the configuration an ACTIVE team's approvers approved has to be
        // approved by them too, so it is REQUESTED here rather than applied: the team
        // keeps its description, strategy, approvers, version id and last update time, and
        // reports the requested version until the team approves it and it is activated.
        if status == crate::state::TeamStatus::Active && changes_configuration {
            // Nothing is written until every instant this handshake records can be
            // computed, so a requested change can never be left waiting on an approval
            // that is owed at no instant at all.
            if now
                .checked_add(crate::state::UPDATE_APPROVAL_DELAY)
                .is_none()
            {
                return Err(UpdateApprovalTeamErrorModeled::InternalServerException(
                    InternalServerException {
                        message: "the update's approval instant could not be computed".to_string(),
                    },
                )
                .into());
            }

            // The same rule for the invitation an approver this change ADDS is owed: the
            // approver set is built after the update session is recorded, so the instant
            // its response comes due is checked here, through the one helper that computes
            // it (`crate::state::invitation_response_due`), before anything is written.
            if input.approvers.is_some()
                && crate::state::invitation_response_due(now).is_none()
            {
                return Err(UpdateApprovalTeamErrorModeled::InternalServerException(
                    InternalServerException {
                        message: crate::state::UNCOMPUTABLE_INVITATION_RESPONSE.to_string(),
                    },
                )
                .into());
            }

            // The store is bounded, so a caller looping on this operation cannot grow
            // this process's memory without end. The predicate and its sentence live once,
            // beside the bound itself in `crate::state`; this operation declares
            // `ServiceQuotaExceededException` (402), which is the error for exactly this
            // condition.
            if let Some(message) = state.session_ceiling_refusal() {
                return Err(
                    UpdateApprovalTeamErrorModeled::ServiceQuotaExceededException(
                        ServiceQuotaExceededException { message },
                    )
                    .into(),
                );
            }

            // The session asks the approvers the team has NOW: they are the ones whose
            // approval the change needs.
            let session_approvers: Vec<crate::state::SessionApprover> = team_record
                .approvers
                .iter()
                .map(|approver| crate::state::SessionApprover {
                    approver_id: approver.approver_id.clone(),
                    identity_id: approver.primary_identity_id.clone(),
                    identity_source_arn: approver.primary_identity_source_arn.clone(),
                    // Nothing records a vote, so every approver starts unanswered; the
                    // approval the clock owes is what answers for them.
                    response: crate::state::SessionResponse::NoResponse,
                    response_time: None,
                })
                .collect();
            // The one path every session of this service is recorded through
            // (`crate::state::ComAmazonawsMpaAwsfluffycoreserviceState::start_session`),
            // so the update session and the baseline session differ only in their kind and
            // their approvers — and a session that cannot be built writes nothing.
            let Some(session_arn) = state.start_session(
                crate::state::SessionKind::Update,
                &team_record,
                session_approvers,
                crate::state::request_region(ctx.parts()),
                now,
            ) else {
                return Err(UpdateApprovalTeamErrorModeled::InternalServerException(
                    InternalServerException {
                        message: "the session's expiration instant could not be computed"
                            .to_string(),
                    },
                )
                .into());
            };

            // Every refusal is behind us, so the approver set the change produces can be
            // built: the ids it mints are consumed by a change that is really recorded.
            let mut requested_approvers = requested_approver_set(
                &mut state,
                input.approvers.as_ref(),
                &existing_approvers,
                now,
            )?
            .unwrap_or(existing_approvers);
            if synchronize_mfa_devices {
                for approver in &mut requested_approvers {
                    approver.synchronize_mfa_devices();
                }
            }
            let team = state.team_mut(&input.arn).ok_or_else(not_found)?;
            // An update action is applied where it was asked for, to the team as it
            // stands: it changes no member the team's approvers approved. The set the
            // change produces is synchronized above, because it is not on the team yet.
            //
            // The live version id and `LastUpdateTime` stay where the approvers left
            // them: the ONE version id this request answers with is the REQUESTED one
            // (`version` below), so moving the live one here would make a version no
            // response ever announced the team's live one, while the configuration it
            // names is still the one awaiting the team's approval.
            if synchronize_mfa_devices {
                team.synchronize_mfa_devices();
            }
            let version = team.mint_version();
            team.request_update(crate::state::PendingTeamUpdate::requested(
                version,
                input.description.clone().unwrap_or(current_description),
                new_strategy.unwrap_or(current_strategy),
                requested_approvers,
                session_arn,
                now,
            ));
            // The version id the caller is answered with is the REQUESTED one: it is the
            // version the team will carry once this change is approved and activated.
            return Ok(UpdateApprovalTeamOutput {
                version_id: Some(version.to_string()),
            });
        }

        // A team whose configuration has never been activated — and one that is no longer
        // active — has no approved configuration to protect, so the change is applied
        // where it was asked for.
        // Nothing can refuse this request now, so the approver set it produces — and the
        // ids minting it consumes — is built here.
        let new_approvers = requested_approver_set(
            &mut state,
            input.approvers.as_ref(),
            &existing_approvers,
            now,
        )?;
        let team = state.team_mut(&input.arn).ok_or_else(not_found)?;
        let mut applied = false;
        if let Some(description) = input.description.as_ref() {
            team.description = description.clone();
            applied = true;
        }
        if let Some(strategy) = new_strategy {
            team.approval_strategy = strategy;
            applied = true;
        }
        if let Some(approvers) = new_approvers {
            team.approvers = approvers;
            applied = true;
        }
        // An update action is applied where it was asked for, to every approver record the
        // team holds — a device belongs to the approver's identity, so a version this team
        // is still waiting on carries the same synchronized record its live set does.
        if synchronize_mfa_devices {
            team.synchronize_mfa_devices();
            applied = true;
        }
        if applied {
            team.bump_version(now);
        }

        Ok(UpdateApprovalTeamOutput {
            version_id: Some(team.version_id.clone()),
        })
    }
}

/// The approver set this request produces, or `Ok(None)` when it names no `Approvers` and
/// the team therefore keeps the set it has.
///
/// Built through the one store path that mints an approver id
/// ([`crate::state::ComAmazonawsMpaAwsfluffycoreserviceState::invite_requested_approvers`]),
/// which ADVANCES the store's approver sequence: this is called only once every refusal
/// the request can earn has been decided, so a refused request consumes no id and the
/// next accepted one mints the id that follows the last one really recorded.
///
/// That store path computes the instant an invited approver's response comes due with
/// checked arithmetic before it mints anything, so a `now` it cannot add that span to is
/// the modelled server error this operation declares — and writes nothing.
fn requested_approver_set(
    state: &mut crate::state::ComAmazonawsMpaAwsfluffycoreserviceState,
    requested: Option<&Vec<ApprovalTeamRequestApprover>>,
    existing: &[crate::state::Approver],
    now: ::std::time::SystemTime,
) -> Result<Option<Vec<crate::state::Approver>>, UpdateApprovalTeamError> {
    let Some(requested) = requested else {
        return Ok(None);
    };
    let invited = state.invite_requested_approvers(
        requested.iter().map(|approver| {
            (
                approver.primary_identity_id.as_str(),
                approver.primary_identity_source_arn.as_str(),
            )
        }),
        existing,
        now,
    );
    match invited {
        Some(approvers) => Ok(Some(approvers)),
        None => Err(UpdateApprovalTeamErrorModeled::InternalServerException(
            InternalServerException {
                message: crate::state::UNCOMPUTABLE_INVITATION_RESPONSE.to_string(),
            },
        )
        .into()),
    }
}
