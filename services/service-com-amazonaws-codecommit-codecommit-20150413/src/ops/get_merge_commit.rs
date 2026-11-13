// SPDX-License-Identifier: CC0-1.0

//! @op GetMergeCommit
//! Reports the merge of two commits this repository has already made: the two
//! commits the specifiers resolve to, the merge base of their histories, and the
//! commit that merge wrote.
//!
//! REPORTING ONLY. Each specifier is resolved the one way this crate resolves a
//! commit specifier ([`crate::state::Repository::resolve_commit`]), the merge
//! base is the one [`crate::state::merge::merge_base`] computes — the same base
//! every merge of that pair evaluates against — and `mergedCommitId` is read from
//! the merge the repository recorded for that pair of commits
//! ([`crate::state::Repository::merge_commit_of`]), written by whichever merge
//! made it: `CreateUnreferencedMergeCommit`, a branch merge or a pull-request
//! merge. Nothing is computed here that a merge did not already do, so asking the
//! question twice answers the same way and changes nothing.
//!
//! A pair this repository has made no merge of carries no `mergedCommitId` — the
//! member is absent rather than an id that resolves to nothing — which is also
//! what a fast-forward merge of the pair leaves behind: "If the fast-forward
//! merge strategy was used, there is no merge commit."
//!
//! `baseCommitId` is left out where the two histories share no commit at all, or
//! where resolving the base exceeds this emulator's merge allowance: this
//! operation models none of the divergence or comparison-limit errors the merge
//! operations do, so an unanswerable base is reported as no base.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetMergeCommit`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codecommit-codecommit-20150413/smithy-model/com-amazonaws-codecommit-codecommit-20150413.json")
//! ```
//!
//! Returns information about a specified merge commit.
//!
//! ## Input
//!
//! * `conflictDetailLevel` (ConflictDetailLevelTypeEnum, one of: FILE_LEVEL | LINE_LEVEL) —
//!   The level of conflict detail to use. If unspecified, the default FILE_LEVEL is used,
//!   which returns a not-mergeable result if the same file has differences in both branches.
//!   If LINE_LEVEL is specified, a conflict is considered not mergeable if the same file in
//!   both branches has differences on the same line.
//! * `conflictResolutionStrategy` (ConflictResolutionStrategyTypeEnum, one of:
//!   ACCEPT_DESTINATION | ACCEPT_SOURCE | AUTOMERGE | NONE) — Specifies which branch to use
//!   when resolving conflicts, or whether to attempt automatically merging two versions of a
//!   file. The default is NONE, which requires any conflicts to be resolved manually before
//!   the merge operation is successful.
//! * `destinationCommitSpecifier` (CommitName) **required** — The branch, tag, HEAD, or other
//!   fully qualified reference used to identify a commit (for example, a branch name or a
//!   full commit ID).
//! * `repositoryName` (RepositoryName, len 1..100, pattern ^[\w\.-]+$) **required** — The
//!   name of the repository that contains the merge commit about which you want to get
//!   information.
//! * `sourceCommitSpecifier` (CommitName) **required** — The branch, tag, HEAD, or other
//!   fully qualified reference used to identify a commit (for example, a branch name or a
//!   full commit ID).
//!
//! ## Output
//!
//! * `baseCommitId` (ObjectId) — The commit ID of the merge base.
//! * `destinationCommitId` (ObjectId) — The commit ID of the destination commit specifier
//!   that was used in the merge evaluation.
//! * `mergedCommitId` (ObjectId) — The commit ID for the merge commit created when the source
//!   branch was merged into the destination branch. If the fast-forward merge strategy was
//!   used, there is no merge commit.
//! * `sourceCommitId` (ObjectId) — The commit ID of the source commit specifier that was used
//!   in the merge evaluation.
//!
//! ## Errors
//!
//! * `CommitDoesNotExistException` (HTTP 400) — The specified commit does not exist or no
//!   commit was specified, and the specified repository has no default branch.
//! * `CommitRequiredException` (HTTP 400) — A commit was not specified.
//! * `EncryptionIntegrityChecksFailedException` (HTTP 500) — An encryption integrity check
//!   failed.
//! * `EncryptionKeyAccessDeniedException` (HTTP 400) — An encryption key could not be
//!   accessed.
//! * `EncryptionKeyDisabledException` (HTTP 400) — The encryption key is disabled.
//! * `EncryptionKeyNotFoundException` (HTTP 400) — No encryption key was found.
//! * `EncryptionKeyUnavailableException` (HTTP 400) — The encryption key is not available.
//! * `InvalidCommitException` (HTTP 400) — The specified commit is not valid.
//! * `InvalidConflictDetailLevelException` (HTTP 400) — The specified conflict detail level
//!   is not valid.
//! * `InvalidConflictResolutionStrategyException` (HTTP 400) — The specified conflict
//!   resolution strategy is not valid.
//! * `InvalidRepositoryNameException` (HTTP 400) — A specified repository name is not valid.
//!   This exception occurs only when a specified repository name is not valid. Other
//!   exceptions occur when a required repository parameter is missing, or when a specified
//!   repository does not exist.
//! * `RepositoryDoesNotExistException` (HTTP 400) — The specified repository does not exist.
//! * `RepositoryNameRequiredException` (HTTP 400) — A repository name is required, but was
//!   not specified.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetMergeCommitError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codecommit-codecommit-20150413.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetMergeCommitError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.codecommit#GetMergeCommit")]
#[derive(Debug)]
pub enum GetMergeCommitErrorModeled {
    CommitDoesNotExistException(CommitDoesNotExistException),
    CommitRequiredException(CommitRequiredException),
    EncryptionIntegrityChecksFailedException(EncryptionIntegrityChecksFailedException),
    EncryptionKeyAccessDeniedException(EncryptionKeyAccessDeniedException),
    EncryptionKeyDisabledException(EncryptionKeyDisabledException),
    EncryptionKeyNotFoundException(EncryptionKeyNotFoundException),
    EncryptionKeyUnavailableException(EncryptionKeyUnavailableException),
    InvalidCommitException(InvalidCommitException),
    InvalidConflictDetailLevelException(InvalidConflictDetailLevelException),
    InvalidConflictResolutionStrategyException(InvalidConflictResolutionStrategyException),
    InvalidRepositoryNameException(InvalidRepositoryNameException),
    RepositoryDoesNotExistException(RepositoryDoesNotExistException),
    RepositoryNameRequiredException(RepositoryNameRequiredException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_merge_commit::GetMergeCommitError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetMergeCommitError = crate::scaffolded::Scaffolded<GetMergeCommitErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetMergeCommitErrorModeled> for GetMergeCommitError {
    fn from(modeled: GetMergeCommitErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetMergeCommit` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetMergeCommit<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetMergeCommit<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetMergeCommit<Eng>
{
    type Input = GetMergeCommitInput;
    type Output = GetMergeCommitOutput;
    type Error = GetMergeCommitError;

    const NAME: &'static str = "GetMergeCommit";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_MERGE_COMMIT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_MERGE_COMMIT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;

        // Both specifiers are required members, so the boundary already refused an
        // absent one; a blank one names no commit either.
        let source_specifier = input.source_commit_specifier.trim();
        let destination_specifier = input.destination_commit_specifier.trim();
        if source_specifier.is_empty() || destination_specifier.is_empty() {
            return Err(GetMergeCommitErrorModeled::CommitRequiredException(
                CommitRequiredException {
                    message: Some("a commit was not specified".to_string()),
                },
            )
            .into());
        }

        // A value outside `ConflictDetailLevelTypeEnum` or
        // `ConflictResolutionStrategyTypeEnum` is already refused at the boundary
        // (see `crate::service`); these keep the handler's own invariant explicit,
        // and apply the documented defaults — "if unspecified, the default
        // FILE_LEVEL is used" and the default strategy "is NONE" — for a request
        // that names neither.
        let conflict_detail_level = input
            .conflict_detail_level
            .as_deref()
            .unwrap_or(crate::state::merge::CONFLICT_DETAIL_FILE_LEVEL);
        if !crate::state::merge::CONFLICT_DETAIL_LEVELS.contains(&conflict_detail_level) {
            return Err(
                GetMergeCommitErrorModeled::InvalidConflictDetailLevelException(
                    InvalidConflictDetailLevelException {
                        message: Some(format!(
                            "the conflict detail level {conflict_detail_level} is not valid"
                        )),
                    },
                )
                .into(),
            );
        }
        let conflict_resolution_strategy = input
            .conflict_resolution_strategy
            .as_deref()
            .unwrap_or(crate::state::merge::CONFLICT_RESOLUTION_NONE);
        if !crate::state::merge::CONFLICT_RESOLUTION_STRATEGIES
            .contains(&conflict_resolution_strategy)
        {
            return Err(
                GetMergeCommitErrorModeled::InvalidConflictResolutionStrategyException(
                    InvalidConflictResolutionStrategyException {
                        message: Some(format!(
                            "the conflict resolution strategy {conflict_resolution_strategy} is not valid"
                        )),
                    },
                )
                .into(),
            );
        }

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            // An unreachable store is an internal mounting fault, not a modelled
            // CodeCommit failure; every handler in this crate refuses it this way.
            crate::scaffolded::Scaffolded::validation("the repository store is unavailable")
        })?;
        // Reporting only: every value below is read from stored references,
        // history and merge records, so the repository is left exactly as the next
        // caller would have found it.
        let store = state.lock();
        let repository = store
            .repositories
            .get(&input.repository_name)
            .ok_or_else(|| {
                GetMergeCommitErrorModeled::RepositoryDoesNotExistException(
                    RepositoryDoesNotExistException {
                        message: Some(crate::state::repository_does_not_exist_message(
                            &input.repository_name,
                        )),
                    },
                )
            })?;

        let commit_does_not_exist = |specifier: &str| {
            GetMergeCommitErrorModeled::CommitDoesNotExistException(CommitDoesNotExistException {
                message: Some(crate::state::commit_does_not_exist_message(specifier)),
            })
        };
        // "The branch, tag, HEAD, or other fully qualified reference used to
        // identify a commit", resolved the one way this crate resolves a commit
        // specifier.
        let source_commit_id = repository
            .resolve_commit(source_specifier)
            .ok_or_else(|| commit_does_not_exist(source_specifier))?
            .commit_id
            .clone();
        let destination_commit_id = repository
            .resolve_commit(destination_specifier)
            .ok_or_else(|| commit_does_not_exist(destination_specifier))?
            .commit_id
            .clone();

        // The merge base of the two resolved commits: the base every merge of
        // that pair evaluates against. The walk is caller-driven CPU work under
        // this service's store lock, so it is charged to one budget and moved off
        // the async worker exactly as the merge operations charge theirs; a base
        // the walk cannot answer (the two histories share no commit, or the
        // allowance ran out) is reported as NO base, because this operation models
        // none of the divergence or comparison-limit errors.
        let base_commit_id = crate::service::off_the_async_worker(|| {
            let mut budget = crate::state::merge::MergeBudget::for_one_request();
            crate::state::merge::merge_base(
                repository,
                &source_commit_id,
                &destination_commit_id,
                &mut budget,
            )
        })
        .ok();

        Ok(GetMergeCommitOutput {
            // "The commit ID for the merge commit created when the source branch
            // was merged into the destination branch": the merge this repository
            // recorded for that pair of commits, and nothing where it has made
            // none of them.
            merged_commit_id: repository
                .merge_commit_of(&source_commit_id, &destination_commit_id)
                .map(str::to_string),
            source_commit_id: Some(source_commit_id),
            destination_commit_id: Some(destination_commit_id),
            base_commit_id,
        })
    }
}
